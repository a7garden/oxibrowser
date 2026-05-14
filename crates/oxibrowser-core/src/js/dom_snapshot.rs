//! DOM snapshot for JS↔DOM bridge.
//!
//! Provides a `Send + Sync + Serialize` representation of the DOM tree
//! that can be passed between the main thread and the JS thread via channels.
//!
//! Architecture:
//! ```text
//! Frame's Document → DomSnapshot::from_frame() → JsCommand::SetDom → JS thread
//!                                                              ↓
//! JS: document.querySelector('a') → DomSnapshot::query_selector() → result
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// DOM 변경 사항
///
/// Records mutations applied to the DOM so they can be replayed,
/// inspected, or transmitted over the CDP protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DomMutation {
    /// Set an attribute on a node.
    SetAttribute {
        node_id: u32,
        name: String,
        value: String,
    },
    /// Set the text content of a node.
    SetTextContent { node_id: u32, text: String },
    /// Simulate a click on an element.
    ClickElement { node_id: u32 },
    /// Input text into a form element.
    InputElement { node_id: u32, value: String },
    /// Create a new element node.
    CreateElement {
        node_id: u32,
        tag: String,
    },
    /// Create a new text node.
    CreateTextNode {
        node_id: u32,
        text: String,
    },
    /// Append a child node to a parent.
    AppendChild {
        parent_id: u32,
        child_id: u32,
    },
    /// Remove a child node from its parent.
    RemoveChild {
        parent_id: u32,
        child_id: u32,
    },
    /// Set innerHTML of an element (parse + replace children).
    SetInnerHtml {
        node_id: u32,
        html: String,
    },
}

/// Serializable DOM node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomNode {
    pub id: u32,
    pub tag: String,
    pub attributes: HashMap<String, String>,
    pub text_content: String,
    pub children: Vec<u32>,
    pub parent: Option<u32>,
    /// 1 = Element, 3 = Text, 8 = Comment, 9 = Document.
    pub node_type: u8,
}

/// DOM tree snapshot (Send + Serialize).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomSnapshot {
    pub url: String,
    pub title: String,
    pub nodes: HashMap<u32, DomNode>,
    pub root_id: u32,
    pub body_id: Option<u32>,
    pub head_id: Option<u32>,
}

impl DomSnapshot {
    /// Create an empty snapshot (no document loaded).
    pub fn empty() -> Self {
        Self {
            url: String::new(),
            title: String::new(),
            nodes: HashMap::new(),
            root_id: 0,
            body_id: None,
            head_id: None,
        }
    }

    /// Extract a snapshot from a Frame's Document.
    ///
    /// Walks all nodes in the document tree, converting each to a `DomNode`.
    pub fn from_frame(frame: &crate::frame::Frame) -> Self {
        let doc = frame.document();
        let tree = doc.tree();
        let url = frame.url().to_string();
        let title = frame.extract_title().unwrap_or_default();

        let mut nodes = HashMap::new();
        let mut body_id = None;
        let mut head_id = None;

        // Walk all nodes via DFS from root
        if let Some(root) = tree.root() {
            collect_nodes(root, doc, tree, &mut nodes, &mut body_id, &mut head_id);
        }

        let root_id = tree.root().map(|id| id.0 as u32).unwrap_or(0);

        Self {
            url,
            title,
            nodes,
            root_id,
            body_id,
            head_id,
        }
    }

    /// Query the first matching node by CSS selector.
    ///
    /// Supports:
    /// - Tag name: `"a"`, `"div"`, `"p"`
    /// - Class: `".classname"`
    /// - ID: `"#id"`
    /// - Tag + class: `"div.class"`
    /// - Tag + ID: `"div#id"`
    /// - Attribute: `"[href]"`, `"a[href]"`
    pub fn query_selector(&self, selector: &str) -> Option<u32> {
        // Walk nodes in tree order (DFS from root)
        let mut stack = vec![self.root_id];
        while let Some(id) = stack.pop() {
            if let Some(node) = self.nodes.get(&id) {
                if self.node_matches_selector(node, selector) {
                    return Some(id);
                }
                // Push children in reverse order so first child is processed first
                for &child in node.children.iter().rev() {
                    stack.push(child);
                }
            }
        }
        None
    }

    /// Query all matching nodes by CSS selector.
    pub fn query_selector_all(&self, selector: &str) -> Vec<u32> {
        let mut results = Vec::new();
        let mut stack = vec![self.root_id];
        while let Some(id) = stack.pop() {
            if let Some(node) = self.nodes.get(&id) {
                if self.node_matches_selector(node, selector) {
                    results.push(id);
                }
                for &child in node.children.iter().rev() {
                    stack.push(child);
                }
            }
        }
        // Reverse to maintain document order (since we used a stack)
        results.reverse();
        results
    }

    /// Get an element by its ID attribute.
    pub fn get_element_by_id(&self, id: &str) -> Option<u32> {
        self.nodes
            .values()
            .find(|node| {
                node.node_type == 1 && node.attributes.get("id").map(|s| s.as_str()) == Some(id)
            })
            .map(|n| n.id)
    }

    /// Get all elements by tag name.
    pub fn get_elements_by_tag_name(&self, tag: &str) -> Vec<u32> {
        let tag_lower = tag.to_lowercase();
        let mut results = Vec::new();
        let mut stack = vec![self.root_id];
        while let Some(id) = stack.pop() {
            if let Some(node) = self.nodes.get(&id) {
                if node.node_type == 1 && node.tag.to_lowercase() == tag_lower {
                    results.push(id);
                }
                for &child in node.children.iter().rev() {
                    stack.push(child);
                }
            }
        }
        results.reverse();
        results
    }

    /// Get all elements by class name.
    pub fn get_elements_by_class_name(&self, class: &str) -> Vec<u32> {
        let mut results = Vec::new();
        let mut stack = vec![self.root_id];
        while let Some(id) = stack.pop() {
            if let Some(node) = self.nodes.get(&id) {
                if node.node_type == 1 {
                    if let Some(cls) = node.attributes.get("class") {
                        if cls.split_whitespace().any(|c| c == class) {
                            results.push(id);
                        }
                    }
                }
                for &child in node.children.iter().rev() {
                    stack.push(child);
                }
            }
        }
        results.reverse();
        results
    }

    /// Check if a node matches a CSS selector.
    fn node_matches_selector(&self, node: &DomNode, selector: &str) -> bool {
        if node.node_type != 1 {
            return false;
        }

        // Check for attribute selector: "a[href]" or "[href]"
        let selector = selector.trim();

        // Extract attribute part if present
        if let Some(bracket_start) = selector.find('[') {
            if let Some(bracket_end) = selector.find(']') {
                if bracket_start < bracket_end {
                    let tag_part = &selector[..bracket_start];
                    let attr_part = &selector[bracket_start + 1..bracket_end];

                    // Check tag part matches (if any)
                    if !tag_part.is_empty() && !node.tag.eq_ignore_ascii_case(tag_part) {
                        return false;
                    }

                    // Check attribute: "href" or "href=value" or "href='value'"
                    let attr_name;
                    let attr_value;
                    if let Some(eq_pos) = attr_part.find('=') {
                        attr_name = &attr_part[..eq_pos];
                        let val = &attr_part[eq_pos + 1..];
                        // Strip quotes
                        attr_value = val.trim_matches('\'').trim_matches('"');
                    } else {
                        attr_name = attr_part;
                        attr_value = "";
                    }

                    let has_attr = node.attributes.contains_key(attr_name);
                    if attr_value.is_empty() {
                        return has_attr;
                    } else {
                        return has_attr
                            && node.attributes.get(attr_name).map(|s| s.as_str())
                                == Some(attr_value);
                    }
                }
            }
        }

        // ID selector: #foo
        if let Some(id) = selector.strip_prefix('#') {
            return node.attributes.get("id").map(|s| s.as_str()) == Some(id);
        }

        // Class selector: .foo
        if let Some(class) = selector.strip_prefix('.') {
            return node
                .attributes
                .get("class")
                .map(|cls| cls.split_whitespace().any(|c| c == class))
                .unwrap_or(false);
        }

        // Tag with class: tag.class
        if let Some(dot_pos) = selector.find('.') {
            let tag_part = &selector[..dot_pos];
            let class_part = &selector[dot_pos + 1..];
            return node.tag.eq_ignore_ascii_case(tag_part)
                && node
                    .attributes
                    .get("class")
                    .map(|cls| cls.split_whitespace().any(|c| c == class_part))
                    .unwrap_or(false);
        }

        // Tag with ID: tag#id
        if let Some(hash_pos) = selector.find('#') {
            let tag_part = &selector[..hash_pos];
            let id_part = &selector[hash_pos + 1..];
            return node.tag.eq_ignore_ascii_case(tag_part)
                && node.attributes.get("id").map(|s| s.as_str()) == Some(id_part);
        }

        // Simple tag name
        node.tag.eq_ignore_ascii_case(selector)
    }
}

/// Recursively collect all nodes from the document tree into the snapshot.
fn collect_nodes(
    node_id: oxibrowser_webapi::dom::NodeId,
    doc: &oxibrowser_webapi::dom::Document,
    tree: &oxibrowser_webapi::dom::Tree,
    nodes: &mut HashMap<u32, DomNode>,
    body_id: &mut Option<u32>,
    head_id: &mut Option<u32>,
) {
    use oxibrowser_webapi::dom::NodeType;

    let id_u32 = node_id.0 as u32;

    if let Some(node) = doc.get_node(node_id) {
        let (tag, attributes, node_type_u8) = match &node.node_type {
            NodeType::Document => (String::new(), HashMap::new(), 9u8),
            NodeType::Element { tag, attributes } => {
                let tag_lower = tag.to_lowercase();
                if tag_lower == "body" && body_id.is_none() {
                    *body_id = Some(id_u32);
                } else if tag_lower == "head" && head_id.is_none() {
                    *head_id = Some(id_u32);
                }
                let attrs: HashMap<String, String> = attributes
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                (tag.clone(), attrs, 1u8)
            }
            NodeType::Text(_) => (String::new(), HashMap::new(), 3u8),
            NodeType::Comment(_) => (String::new(), HashMap::new(), 8u8),
            NodeType::Doctype { .. } => (String::new(), HashMap::new(), 10u8),
        };

        // Collect text content from direct text children
        let text_content = collect_text_content(node_id, doc, tree);

        let children: Vec<u32> = tree.children(node_id).iter().map(|c| c.0 as u32).collect();
        let parent = tree.parent(node_id).map(|p| p.0 as u32);

        let dom_node = DomNode {
            id: id_u32,
            tag,
            attributes,
            text_content,
            children,
            parent,
            node_type: node_type_u8,
        };

        nodes.insert(id_u32, dom_node);

        // Recurse into children
        for &child in tree.children(node_id) {
            collect_nodes(child, doc, tree, nodes, body_id, head_id);
        }
    }
}

/// Collect text content from a node's direct text children and their descendants.
fn collect_text_content(
    node_id: oxibrowser_webapi::dom::NodeId,
    doc: &oxibrowser_webapi::dom::Document,
    tree: &oxibrowser_webapi::dom::Tree,
) -> String {
    let mut text = String::new();
    collect_text_recursive(node_id, doc, tree, &mut text);
    text
}

fn collect_text_recursive(
    node_id: oxibrowser_webapi::dom::NodeId,
    doc: &oxibrowser_webapi::dom::Document,
    tree: &oxibrowser_webapi::dom::Tree,
    text: &mut String,
) {
    use oxibrowser_webapi::dom::NodeType;
    if let Some(node) = doc.get_node(node_id) {
        if let NodeType::Text(t) = &node.node_type {
            text.push_str(t);
        }
    }
    for &child in tree.children(node_id) {
        collect_text_recursive(child, doc, tree, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::Frame;
    use url::Url;

    fn make_frame(html: &str) -> Frame {
        let url = Url::parse("https://example.com").unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(Frame::from_html(url, html)).unwrap()
    }

    #[test]
    fn test_dom_snapshot_from_frame() {
        let html = r#"<html><head><title>Test Page</title></head>
            <body><p class="intro">Hello</p><a href="/link">click</a></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);

        assert_eq!(snapshot.url, "https://example.com/");
        assert_eq!(snapshot.title, "Test Page");
        assert!(snapshot.body_id.is_some(), "should find body element");
        assert!(snapshot.head_id.is_some(), "should find head element");
        assert!(snapshot.nodes.len() > 5, "should have multiple nodes");
    }

    #[test]
    fn test_query_selector_tag() {
        let html = r#"<html><body><p>first</p><p>second</p></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);

        let found = snapshot.query_selector("p");
        assert!(found.is_some(), "should find a <p> element");
        let node = snapshot.nodes.get(&found.unwrap()).unwrap();
        assert_eq!(node.tag, "p");
    }

    #[test]
    fn test_query_selector_class() {
        let html = r#"<html><body><div class="foo">content</div></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);

        let found = snapshot.query_selector(".foo");
        assert!(found.is_some(), "should find element with class .foo");
    }

    #[test]
    fn test_query_selector_id() {
        let html = r#"<html><body><span id="bar">text</span></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);

        let found = snapshot.query_selector("#bar");
        assert!(found.is_some(), "should find element with id #bar");
    }

    #[test]
    fn test_query_selector_tag_class() {
        let html =
            r#"<html><body><div class="main">main</div><p class="main">para</p></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);

        let found = snapshot.query_selector("div.main");
        assert!(found.is_some(), "should find div.main");
        let node = snapshot.nodes.get(&found.unwrap()).unwrap();
        assert_eq!(node.tag, "div");
    }

    #[test]
    fn test_query_selector_tag_id() {
        let html = r#"<html><body><div id="content">c</div></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);

        let found = snapshot.query_selector("div#content");
        assert!(found.is_some(), "should find div#content");
    }

    #[test]
    fn test_query_selector_attribute() {
        let html = r#"<html><body><a href="/link">click</a><p>no link</p></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);

        let found = snapshot.query_selector("[href]");
        assert!(found.is_some(), "should find element with href attribute");
        let node = snapshot.nodes.get(&found.unwrap()).unwrap();
        assert_eq!(node.tag, "a");

        let found2 = snapshot.query_selector("a[href]");
        assert!(found2.is_some(), "should find a[href]");
    }

    #[test]
    fn test_query_selector_all() {
        let html = "<html><body><ul><li>a</li><li>b</li><li>c</li></ul></body></html>";
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);

        let items = snapshot.query_selector_all("li");
        assert_eq!(items.len(), 3, "should find 3 <li> elements");
    }

    #[test]
    fn test_get_element_by_id() {
        let html = r#"<html><body><div id="main">content</div></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);

        let found = snapshot.get_element_by_id("main");
        assert!(found.is_some(), "should find element by id");

        let not_found = snapshot.get_element_by_id("nonexistent");
        assert!(not_found.is_none(), "should not find nonexistent id");
    }

    #[test]
    fn test_get_elements_by_tag_name() {
        let html = "<html><body><p>a</p><p>b</p><p>c</p></body></html>";
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);

        let items = snapshot.get_elements_by_tag_name("p");
        assert_eq!(items.len(), 3, "should find 3 <p> elements");
    }

    #[test]
    fn test_get_elements_by_class_name() {
        let html =
            r#"<html><body><div class="item">a</div><div class="item">b</div></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);

        let items = snapshot.get_elements_by_class_name("item");
        assert_eq!(items.len(), 2, "should find 2 .item elements");
    }

    #[test]
    fn test_dom_snapshot_empty() {
        let snapshot = DomSnapshot::empty();
        assert!(snapshot.url.is_empty());
        assert!(snapshot.nodes.is_empty());
        assert!(snapshot.body_id.is_none());
    }

    #[test]
    fn test_node_text_content() {
        let html = r#"<html><body><p>Hello World</p></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);

        let p_id = snapshot.query_selector("p").unwrap();
        let p_node = snapshot.nodes.get(&p_id).unwrap();
        assert!(
            p_node.text_content.contains("Hello World"),
            "text content should include 'Hello World'"
        );
    }

    #[test]
    fn test_node_parent_child_relationship() {
        let html = "<html><body><div><p>text</p></div></body></html>";
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);

        let div_id = snapshot.query_selector("div").unwrap();
        let p_id = snapshot.query_selector("p").unwrap();

        let div_node = snapshot.nodes.get(&div_id).unwrap();
        let p_node = snapshot.nodes.get(&p_id).unwrap();

        assert!(
            div_node.children.contains(&p_id),
            "div should have p as child"
        );
        assert_eq!(p_node.parent, Some(div_id), "p's parent should be div");
    }
}
