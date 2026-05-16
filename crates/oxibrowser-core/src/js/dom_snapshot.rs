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

    /// Query selector scoped to a subtree rooted at `root_id`.
    /// Skips the root node itself — only searches descendants.
    pub fn query_selector_from(&self, root_id: u32, selector: &str) -> Option<u32> {
        let mut stack = vec![root_id];
        while let Some(id) = stack.pop() {
            if let Some(node) = self.nodes.get(&id) {
                if id != root_id && self.node_matches_selector(node, selector) {
                    return Some(id);
                }
                for &child in node.children.iter().rev() {
                    stack.push(child);
                }
            }
        }
        None
    }

    /// Query all matching nodes scoped to a subtree rooted at `root_id`.
    /// Skips the root node itself — only searches descendants.
    pub fn query_selector_all_from(&self, root_id: u32, selector: &str) -> Vec<u32> {
        let mut results = Vec::new();
        let mut stack = vec![root_id];
        while let Some(id) = stack.pop() {
            if let Some(node) = self.nodes.get(&id) {
                if id != root_id && self.node_matches_selector(node, selector) {
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

    // -----------------------------------------------------------------------
    // Structured data extraction (for OXI.getStructuredPage)
    // -----------------------------------------------------------------------

    /// Extract all headings from the document.
    ///
    /// Returns a list of `(level, text)` tuples where level is 1–6.
    /// Includes both `<h1>`–`<h6>` tags and elements with `role="heading"`.
    pub fn headings(&self) -> Vec<(u8, String)> {
        let heading_tags = ["h1", "h2", "h3", "h4", "h5", "h6"];
        let mut result = Vec::new();
        let mut stack = vec![self.root_id];
        while let Some(id) = stack.pop() {
            if let Some(node) = self.nodes.get(&id) {
                if node.node_type == 1 {
                    let tag_lower = node.tag.to_lowercase();
                    if let Some(idx) = heading_tags.iter().position(|t| *t == tag_lower) {
                        result.push((idx as u8 + 1, self.deep_text_content(node.id)));
                    } else if node.attributes.get("role").map(|s| s.as_str()) == Some("heading") {
                        let level: u8 = node.attributes
                            .get("aria-level")
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(2);
                        result.push((level.clamp(1, 6), self.deep_text_content(node.id)));
                    }
                }
                for &child in node.children.iter().rev() {
                    stack.push(child);
                }
            }
        }
        // No reverse needed: children are pushed in reverse so first child
        // is popped first, producing correct document order.
        result
    }

    /// Extract all links from the document.
    ///
    /// Returns `(text, href)` pairs.
    pub fn links(&self) -> Vec<(String, String)> {
        let mut result = Vec::new();
        let mut stack = vec![self.root_id];
        while let Some(id) = stack.pop() {
            if let Some(node) = self.nodes.get(&id) {
                if node.node_type == 1 && node.tag.to_lowercase() == "a" {
                    let href = node.attributes.get("href").cloned().unwrap_or_default();
                    let text = self.deep_text_content(node.id);
                    result.push((text, href));
                }
                for &child in node.children.iter().rev() {
                    stack.push(child);
                }
            }
        }
        // No reverse: children pushed in reverse → first child popped first → document order.
        result
    }

    /// Extract meta tags from the document.
    ///
    /// Returns name/property → content pairs (e.g. description, og:title).
    pub fn meta_tags(&self) -> HashMap<String, String> {
        let mut result = HashMap::new();
        for node in self.nodes.values() {
            if node.node_type == 1 && node.tag.to_lowercase() == "meta" {
                let name = node.attributes.get("name")
                    .or_else(|| node.attributes.get("property"));
                let content = node.attributes.get("content");
                if let (Some(n), Some(c)) = (name, content) {
                    result.insert(n.clone(), c.clone());
                }
            }
        }
        result
    }

    /// Recursively collect all text content from a node and its descendants.
    fn deep_text_content(&self, node_id: u32) -> String {
        let mut text = String::new();
        self.collect_text_recursive(node_id, &mut text);
        text.trim().to_string()
    }

    fn collect_text_recursive(&self, node_id: u32, text: &mut String) {
        if let Some(node) = self.nodes.get(&node_id) {
            if node.node_type == 3 {
                // Text node: text_content holds the text
                text.push_str(&node.text_content);
                text.push(' ');
            } else if node.node_type == 1 {
                // Element node: recurse into children
                for &child in &node.children {
                    self.collect_text_recursive(child, text);
                }
            }
        }
    }

    /// Check if a node matches a CSS selector.
    ///
    /// Supports:
    /// - Universal selector `*`
    /// - Multiple selectors `a, b` (comma-separated)
    /// - Descendant combinator `a b` (space-separated)
    /// - Attribute selectors `[attr]`, `[attr=val]`, `tag[attr]`
    /// - Tag name, `.class`, `#id`, `tag.class`, `tag#id`
    fn node_matches_selector(&self, node: &DomNode, selector: &str) -> bool {
        if node.node_type != 1 {
            return false;
        }

        // Handle comma-separated selectors: match any
        for single_sel in selector.split(',') {
            let single_sel = single_sel.trim();
            if self.node_matches_single(node, single_sel) {
                return true;
            }
        }
        false
    }

    /// Check a single selector (no commas) against a node.
    fn node_matches_single(&self, node: &DomNode, selector: &str) -> bool {
        // Universal selector
        if selector == "*" {
            return true;
        }

        // Descendant combinator: split on whitespace
        let parts: Vec<&str> = selector.split_whitespace().collect();
        if parts.len() > 1 {
            // Last part must match this node
            if !self.matches_simple(node, parts[parts.len() - 1]) {
                return false;
            }
            // Walk ancestors for preceding parts
            let ancestor_parts = &parts[..parts.len() - 1];
            let mut current = node.parent;
            let mut idx = ancestor_parts.len();
            while let Some(parent_id) = current {
                if idx == 0 {
                    return true;
                }
                let ancestor = match self.nodes.get(&parent_id) {
                    Some(a) => a,
                    None => break,
                };
                if self.matches_simple(ancestor, ancestor_parts[idx - 1]) {
                    idx -= 1;
                }
                current = ancestor.parent;
            }
            return idx == 0;
        }

        self.matches_simple(node, selector)
    }

    /// Match a simple selector (no commas, no descendant) against a node.
    fn matches_simple(&self, node: &DomNode, selector: &str) -> bool {
        if node.node_type != 1 {
            return false;
        }

        // Universal selector
        if selector == "*" {
            return true;
        }

        // Check for attribute selector: "a[href]" or "[href]"
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
                    return if let Some(eq_pos) = attr_part.find('=') {
                        let attr_name = &attr_part[..eq_pos];
                        let val = attr_part[eq_pos + 1..]
                            .trim_matches('\'')
                            .trim_matches('"');
                        let has_attr = node.attributes.contains_key(attr_name);
                        has_attr
                            && node.attributes.get(attr_name).map(|s| s.as_str()) == Some(val)
                    } else {
                        node.attributes.contains_key(attr_part)
                    };
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

    /// Get the first child node ID.
    pub fn first_child(&self, node_id: u32) -> Option<u32> {
        self.nodes.get(&node_id)
            .and_then(|n| n.children.first().copied())
    }

    /// Get the last child node ID.
    pub fn last_child(&self, node_id: u32) -> Option<u32> {
        self.nodes.get(&node_id)
            .and_then(|n| n.children.last().copied())
    }

    /// Get the next sibling node ID.
    pub fn next_sibling(&self, node_id: u32) -> Option<u32> {
        let parent_id = self.nodes.get(&node_id).and_then(|n| n.parent)?;
        let parent = self.nodes.get(&parent_id)?;
        let idx = parent.children.iter().position(|&id| id == node_id)?;
        parent.children.get(idx + 1).copied()
    }

    /// Get the previous sibling node ID.
    pub fn previous_sibling(&self, node_id: u32) -> Option<u32> {
        let parent_id = self.nodes.get(&node_id).and_then(|n| n.parent)?;
        let parent = self.nodes.get(&parent_id)?;
        let idx = parent.children.iter().position(|&id| id == node_id)?;
        if idx > 0 {
            parent.children.get(idx - 1).copied()
        } else {
            None
        }
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

    // -----------------------------------------------------------------------
    // Structured data extraction tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_headings_extraction() {
        let html = r#"<html><body>
            <h1>Main Title</h1>
            <h2>Subtitle</h2>
            <h3>Section</h3>
            <p>Not a heading</p>
        </body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);

        let headings = snapshot.headings();
        assert_eq!(headings.len(), 3, "should find 3 headings");
        assert_eq!(headings[0].0, 1, "first heading should be h1");
        assert!(headings[0].1.contains("Main Title"));
        assert_eq!(headings[1].0, 2, "second heading should be h2");
        assert_eq!(headings[2].0, 3, "third heading should be h3");
    }

    #[test]
    fn test_headings_with_aria_role() {
        let html = r#"<html><body>
            <span role="heading" aria-level="2">ARIA Heading</span>
            <div role="heading">Default Level</div>
        </body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);

        let headings = snapshot.headings();
        assert_eq!(headings.len(), 2, "should find 2 ARIA headings");
        assert_eq!(headings[0].0, 2, "first should be level 2");
        assert!(headings[0].1.contains("ARIA Heading"));
        assert_eq!(headings[1].0, 2, "default level should be 2");
    }

    #[test]
    fn test_links_extraction() {
        let html = r#"<html><body>
            <a href="https://example.com">Example</a>
            <a href="/about">About Us</a>
            <a>No href</a>
        </body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);

        let links = snapshot.links();
        assert_eq!(links.len(), 3, "should find 3 links");
        assert_eq!(links[0].1, "https://example.com");
        assert!(links[0].0.contains("Example"));
        assert_eq!(links[1].1, "/about");
        assert_eq!(links[2].1, "", "link without href should have empty string");
    }

    #[test]
    fn test_meta_tags_extraction() {
        let html = r#"<html><head>
            <meta name="description" content="A test page">
            <meta property="og:title" content="OG Title">
            <meta name="viewport" content="width=device-width">
        </head><body></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);

        let meta = snapshot.meta_tags();
        assert_eq!(meta.get("description").unwrap(), "A test page");
        assert_eq!(meta.get("og:title").unwrap(), "OG Title");
        assert_eq!(meta.get("viewport").unwrap(), "width=device-width");
    }

    #[test]
    fn test_structured_data_empty_page() {
        let snapshot = DomSnapshot::empty();
        assert!(snapshot.headings().is_empty());
        assert!(snapshot.links().is_empty());
        assert!(snapshot.meta_tags().is_empty());
    }
}
