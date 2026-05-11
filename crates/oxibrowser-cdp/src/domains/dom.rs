//! CDP DOM domain handler.
//!
//! Handles DOM.getDocument, DOM.querySelector, DOM.querySelectorAll,
//! DOM.getOuterHTML, DOM.describeNode, DOM.resolveNode.

use crate::domains::DomainResult;
use crate::protocol::CdpError;
use serde_json::{json, Value};

/// Dispatch DOM domain methods.
pub fn handle(method: &str, params: Option<Value>) -> DomainResult {
    match method {
        "getDocument" => get_document(),
        "querySelector" => query_selector(params),
        "querySelectorAll" => query_selector_all(params),
        "getOuterHTML" => get_outer_html(params),
        "describeNode" => describe_node(params),
        "resolveNode" => resolve_node(params),
        _ => Err(CdpError {
            code: -32601,
            message: format!("DOM.{} not implemented", method),
        }),
    }
}

/// DOM.getDocument — returns the root DOM node.
fn get_document() -> DomainResult {
    Ok(Some(json!({
        "root": {
            "nodeId": 0,
            "backendNodeId": 0,
            "nodeType": 9,
            "nodeName": "#document",
            "localName": "",
            "nodeValue": "",
            "childNodeCount": 1,
            "children": [
                {
                    "nodeId": 1,
                    "backendNodeId": 1,
                    "nodeType": 1,
                    "nodeName": "HTML",
                    "localName": "html",
                    "nodeValue": "",
                    "childNodeCount": 2,
                    "children": [
                        {
                            "nodeId": 2,
                            "backendNodeId": 2,
                            "nodeType": 1,
                            "nodeName": "HEAD",
                            "localName": "head",
                            "nodeValue": "",
                            "childNodeCount": 0
                        },
                        {
                            "nodeId": 3,
                            "backendNodeId": 3,
                            "nodeType": 1,
                            "nodeName": "BODY",
                            "localName": "body",
                            "nodeValue": "",
                            "childNodeCount": 0
                        }
                    ]
                }
            ]
        }
    })))
}

/// DOM.querySelector — finds a single node matching a CSS selector.
fn query_selector(params: Option<Value>) -> DomainResult {
    let params = params.unwrap_or_default();
    let node_id = params.get("nodeId").and_then(|v| v.as_u64()).unwrap_or(0);
    let _selector = params
        .get("selector")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // In a real implementation, this would query the actual DOM.
    // For now, return a stub result.
    Ok(Some(json!({
        "nodeId": node_id + 100
    })))
}

/// DOM.querySelectorAll — finds all nodes matching a CSS selector.
fn query_selector_all(params: Option<Value>) -> DomainResult {
    let params = params.unwrap_or_default();
    let _node_id = params.get("nodeId").and_then(|v| v.as_u64()).unwrap_or(0);
    let _selector = params
        .get("selector")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    Ok(Some(json!({
        "nodeIds": []
    })))
}

/// DOM.getOuterHTML — returns the outer HTML of a node.
fn get_outer_html(params: Option<Value>) -> DomainResult {
    let params = params.unwrap_or_default();
    let _node_id = params.get("nodeId").and_then(|v| v.as_u64()).unwrap_or(0);

    // In a real implementation, this would serialize the actual DOM node.
    Ok(Some(json!({
        "outerHTML": "<html><head></head><body></body></html>"
    })))
}

/// DOM.describeNode — describes a DOM node.
fn describe_node(params: Option<Value>) -> DomainResult {
    let params = params.unwrap_or_default();
    let node_id = params
        .get("nodeId")
        .and_then(|v| v.as_u64())
        .or_else(|| params.get("backendNodeId").and_then(|v| v.as_u64()))
        .unwrap_or(0);

    Ok(Some(json!({
        "node": {
            "nodeId": node_id,
            "backendNodeId": node_id,
            "nodeType": 1,
            "nodeName": "BODY",
            "localName": "body",
            "nodeValue": "",
            "childNodeCount": 0
        }
    })))
}

/// DOM.resolveNode — resolves a DOM node to a JS remote object.
fn resolve_node(params: Option<Value>) -> DomainResult {
    let params = params.unwrap_or_default();
    let _node_id = params.get("nodeId").and_then(|v| v.as_u64()).unwrap_or(0);

    Ok(Some(json!({
        "object": {
            "type": "object",
            "subtype": "node",
            "className": "HTMLBodyElement",
            "description": "body",
            "objectId": format!("node-{}", uuid::Uuid::new_v4())
        }
    })))
}
