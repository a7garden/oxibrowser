//! CDP DOM domain handler.
//!
//! Handles DOM.getDocument, DOM.querySelector, DOM.querySelectorAll,
//! DOM.getOuterHTML, DOM.describeNode, DOM.resolveNode.

use crate::domains::{DispatchContext, DomainResult};
use crate::protocol::CdpError;
use oxibrowser_webapi::dom::{NodeId, NodeType};
use serde_json::{json, Value};

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
            message: format!("DOM.{} not implemented", method),
        }),
    }
}

/// DOM.getDocument — returns the root DOM node from the real parsed document.
async fn get_document(ctx: &DispatchContext) -> DomainResult {
    let guard = ctx.session.read().await;
    match guard.page() {
        Some(page) => {
            let document = page.root_frame().document();
            let tree = page.root_frame().document().tree();
            let root_id = match tree.root() {
                Some(id) => id,
                None => {
                    return Ok(Some(json!({
                        "root": {
                            "nodeId": 0,
                            "backendNodeId": 0,
                            "nodeType": 9,
                            "nodeName": "#document",
                            "localName": "",
                            "nodeValue": "",
                            "childNodeCount": 0
                        }
                    })))
                }
            };
            Ok(Some(json!({
                "root": build_cdp_node(document, root_id, 0)
            })))
        }
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

    let guard = ctx.session.read().await;
    match guard.page() {
        Some(page) => {
            let frame = page.root_frame();
            match frame.query_selector(selector) {
                Some(found_id) => Ok(Some(json!({
                    "nodeId": found_id.0
                }))),
                None => Ok(Some(json!({
                    "nodeId": 0
                }))),
            }
        }
        None => Ok(Some(json!({
            "nodeId": 0
        }))),
    }
}

/// DOM.querySelectorAll — finds all nodes matching a CSS selector.
async fn query_selector_all(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let params = params.unwrap_or_default();
    let selector = params
        .get("selector")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let guard = ctx.session.read().await;
    match guard.page() {
        Some(page) => {
            let document = page.root_frame().document();
            let node_ids: Vec<u64> = document
                .query_selector_all(selector)
                .iter()
                .map(|id| id.0 as u64)
                .collect();
            Ok(Some(json!({
                "nodeIds": node_ids
            })))
        }
        None => Ok(Some(json!({
            "nodeIds": []
        }))),
    }
}

/// DOM.getOuterHTML — returns the actual HTML from the session's page.
async fn get_outer_html(ctx: &DispatchContext) -> DomainResult {
    let guard = ctx.session.read().await;
    let html = match guard.page() {
        Some(page) => page.content().to_string(),
        None => "<html><head></head><body></body></html>".to_string(),
    };
    Ok(Some(json!({
        "outerHTML": html
    })))
}

/// DOM.describeNode — describes a DOM node with real data from the document tree.
async fn describe_node(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let params = params.unwrap_or_default();
    let node_id_val = params
        .get("nodeId")
        .and_then(|v| v.as_u64())
        .or_else(|| params.get("backendNodeId").and_then(|v| v.as_u64()))
        .unwrap_or(0);

    let node_id = NodeId(node_id_val as usize);

    let guard = ctx.session.read().await;
    match guard.page() {
        Some(page) => {
            let document = page.root_frame().document();
            match document.get_node(node_id) {
                Some(node) => {
                    let (node_type_num, node_name, local_name, node_value) = match &node.node_type {
                        NodeType::Document => {
                            (9, "#document".to_string(), String::new(), String::new())
                        }
                        NodeType::Element { tag, .. } => {
                            (1, tag.to_uppercase(), tag.to_lowercase(), String::new())
                        }
                        NodeType::Text(text) => {
                            (3, "#text".to_string(), String::new(), text.clone())
                        }
                        NodeType::Comment(text) => {
                            (8, "#comment".to_string(), String::new(), text.clone())
                        }
                        NodeType::Doctype { name } => {
                            (10, "#doctype".to_string(), String::new(), name.clone())
                        }
                    };

                    let child_count = document.tree().children(node_id).len();

                    // Collect attribute pairs [name1, value1, name2, value2, ...]
                    let attributes: Vec<Value> =
                        if let NodeType::Element { attributes, .. } = &node.node_type {
                            let mut attrs = Vec::new();
                            for (k, v) in attributes {
                                attrs.push(json!(k));
                                attrs.push(json!(v));
                            }
                            attrs
                        } else {
                            Vec::new()
                        };

                    Ok(Some(json!({
                        "node": {
                            "nodeId": node_id.0,
                            "backendNodeId": node_id.0,
                            "nodeType": node_type_num,
                            "nodeName": node_name,
                            "localName": local_name,
                            "nodeValue": node_value,
                            "childNodeCount": child_count,
                            "attributes": attributes,
                        }
                    })))
                }
                None => Err(CdpError {
                    code: -32000,
                    message: format!("Node not found: {}", node_id.0),
                }),
            }
        }
        None => Err(CdpError {
            code: -32000,
            message: "No active page".to_string(),
        }),
    }
}

/// DOM.resolveNode — resolves a DOM node to a JS remote object.
///
/// Uses deterministic objectId format "oxi-node-{nodeId}" so that
/// Runtime.callFunctionOn can look up the node by its objectId.
fn resolve_node(params: Option<Value>) -> DomainResult {
    let params = params.unwrap_or_default();
    let node_id = params.get("nodeId").and_then(|v| v.as_u64()).unwrap_or(0);
    let object_id = format!("oxi-node-{}", node_id);

    Ok(Some(json!({
        "object": {
            "type": "object",
            "subtype": "node",
            "className": "HTMLElement",
            "description": format!("node#{}", node_id),
            "objectId": object_id
        }
    })))
}

// ---------------------------------------------------------------------------
// CDP node tree builder
// ---------------------------------------------------------------------------

/// Maximum depth for the CDP node tree (avoids huge outputs).
const MAX_CDP_TREE_DEPTH: usize = 10;

/// Build a CDP-compatible JSON node from the webapi DOM tree.
fn build_cdp_node(
    document: &oxibrowser_webapi::dom::Document,
    node_id: NodeId,
    depth: usize,
) -> Value {
    let node = match document.get_node(node_id) {
        Some(n) => n,
        None => return json!({}),
    };

    let (node_type_num, node_name, local_name, node_value) = match &node.node_type {
        NodeType::Document => (9, "#document".to_string(), String::new(), String::new()),
        NodeType::Element { tag, .. } => (1, tag.to_uppercase(), tag.to_lowercase(), String::new()),
        NodeType::Text(text) => (3, "#text".to_string(), String::new(), text.clone()),
        NodeType::Comment(text) => (8, "#comment".to_string(), String::new(), text.clone()),
        NodeType::Doctype { name } => (10, "#doctype".to_string(), String::new(), name.clone()),
    };

    // Collect attribute pairs [name1, value1, name2, value2, ...]
    let attributes: Vec<Value> = if let NodeType::Element { attributes, .. } = &node.node_type {
        let mut attrs = Vec::new();
        for (k, v) in attributes {
            attrs.push(json!(k));
            attrs.push(json!(v));
        }
        attrs
    } else {
        Vec::new()
    };

    let children: Vec<Value> = if depth < MAX_CDP_TREE_DEPTH {
        document
            .tree()
            .children(node_id)
            .iter()
            .filter(|&&child_id| {
                if let Some(child_node) = document.get_node(child_id) {
                    if let NodeType::Text(t) = &child_node.node_type {
                        return !t.trim().is_empty();
                    }
                }
                true
            })
            .map(|&child_id| build_cdp_node(document, child_id, depth + 1))
            .collect()
    } else {
        Vec::new()
    };

    json!({
        "nodeId": node_id.0,
        "backendNodeId": node_id.0,
        "nodeType": node_type_num,
        "nodeName": node_name,
        "localName": local_name,
        "nodeValue": node_value,
        "childNodeCount": children.len(),
        "children": children,
        "attributes": attributes,
    })
}
