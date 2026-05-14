#![allow(unused_variables, dead_code)]
//! Simple text-based DOM renderer.
//!
//! Renders the DOM tree as ASCII/Unicode text for terminal output.
//! Uses a simplified block layout model.

use crate::js::dom_snapshot::DomNode;
use crate::js::dom_snapshot::DomSnapshot;

/// Render a DomSnapshot to a text string.
pub fn render_to_text(snapshot: &DomSnapshot) -> String {
    let mut output = String::new();
    let root_id = snapshot.body_id.or(Some(snapshot.root_id));

    if let Some(root) = root_id.and_then(|id| snapshot.nodes.get(&id)) {
        render_node(snapshot, root, &mut output, 0);
    } else if let Some(root) = snapshot.nodes.get(&snapshot.root_id) {
        render_node(snapshot, root, &mut output, 0);
    }

    output
}

fn render_node(snapshot: &DomSnapshot, node: &DomNode, output: &mut String, depth: usize) {
    match node.node_type {
        3 => {
            // TEXT_NODE
            let text = node.text_content.trim();
            if !text.is_empty() {
                let indent = "  ".repeat(depth);
                for line in text.lines() {
                    output.push_str(&indent);
                    output.push_str(line.trim());
                    output.push('\n');
                }
            }
        }
        1 => {
            // ELEMENT_NODE
            let tag = node.tag.to_uppercase();
            let invisible = matches!(
                tag.as_str(),
                "SCRIPT" | "STYLE" | "LINK" | "META" | "HEAD" | "HTML"
                    | "TITLE" | "NOSCRIPT" | "BASE" | "SVG" | "DEFS" | "PATH"
            );
            if invisible {
                return;
            }

            let is_self_closing = matches!(
                tag.as_str(),
                "BR" | "HR" | "IMG" | "INPUT" | "AREA" | "COL" | "EMBED"
                    | "PARAM" | "SOURCE" | "TRACK" | "WBR"
            );

            if is_self_closing {
                let indent = "  ".repeat(depth);
                output.push_str(&indent);
                if tag == "BR" {
                    output.push('\n');
                } else if tag == "HR" {
                    output.push_str("─────────────────────────────────\n");
                } else if tag == "IMG" {
                    let alt = node
                        .attributes
                        .get("alt")
                        .map(|s| s.as_str())
                        .unwrap_or("img");
                    output.push_str("[IMAGE: ");
                    output.push_str(alt);
                    output.push_str("]\n");
                }
                return;
            }

            let indent = "  ".repeat(depth);
            let is_block = matches!(
                tag.as_str(),
                "DIV" | "P" | "H1" | "H2" | "H3" | "H4" | "H5" | "H6"
                    | "UL" | "OL" | "LI" | "TABLE" | "TR" | "TD" | "TH"
                    | "FORM" | "SECTION" | "ARTICLE" | "HEADER" | "FOOTER"
                    | "NAV" | "MAIN" | "ADDRESS" | "BLOCKQUOTE" | "PRE"
                    | "FIGURE" | "FIGCAPTION" | "ASIDE"
            );

            if is_block && depth > 0 {
                output.push('\n');
            }

            // Open tag
            output.push_str(&indent);
            output.push('<');
            output.push_str(&tag.to_lowercase());
            if let Some(id) = node.attributes.get("id") {
                output.push_str(" id=\"");
                output.push_str(id);
                output.push('"');
            }
            output.push_str(">\n");

            // Render children
            for &child_id in &node.children {
                if let Some(child) = snapshot.nodes.get(&child_id) {
                    render_node(snapshot, child, output, depth + 1);
                }
            }

            // Close tag
            output.push_str(&indent);
            output.push_str("</");
            output.push_str(&tag.to_lowercase());
            output.push_str(">\n");
        }
        _ => {
            // Document, Comment, etc.
            for &child_id in &node.children {
                if let Some(child) = snapshot.nodes.get(&child_id) {
                    render_node(snapshot, child, output, depth);
                }
            }
        }
    }
}

/// Render a DomSnapshot to markdown-friendly plain text.
pub fn render_to_markdown(snapshot: &DomSnapshot) -> String {
    let mut output = String::new();

    if let Some(body_id) = snapshot.body_id {
        if let Some(body) = snapshot.nodes.get(&body_id) {
            render_markdown_node(snapshot, body, &mut output);
        }
    }

    output.trim().to_string()
}

fn render_markdown_node(snapshot: &DomSnapshot, node: &DomNode, output: &mut String) {
    match node.node_type {
        3 => {
            let text = node.text_content.trim();
            if !text.is_empty() {
                output.push_str(text);
                output.push(' ');
            }
        }
        1 => {
            let tag = node.tag.to_uppercase();
            let _skip = matches!(
                tag.as_str(),
                "SCRIPT" | "STYLE" | "LINK" | "META" | "HEAD" | "HTML"
                    | "NOSCRIPT" | "SVG" | "DEFS" | "PATH" | "BASE" | "TITLE"
            );

            for &child_id in &node.children {
                if let Some(child) = snapshot.nodes.get(&child_id) {
                    render_markdown_node(snapshot, child, output);
                }
            }

            if matches!(
                tag.as_str(),
                "DIV" | "P" | "H1" | "H2" | "H3" | "BR" | "LI" | "TR" | "SECTION"
                    | "ARTICLE" | "HEADER" | "FOOTER" | "NAV" | "MAIN" | "BLOCKQUOTE"
            ) {
                output.push('\n');
                output.push('\n');
            }
        }
        _ => {
            for &child_id in &node.children {
                if let Some(child) = snapshot.nodes.get(&child_id) {
                    render_markdown_node(snapshot, child, output);
                }
            }
        }
    }
}
