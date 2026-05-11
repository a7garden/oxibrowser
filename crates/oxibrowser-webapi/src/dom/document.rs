//! Document — the root of a parsed DOM tree.
//!
//! Uses html5ever (from Servo) to parse HTML and build a DOM tree.

use crate::dom::node::{Node, NodeId, NodeType};
use crate::dom::tree::Tree;

use html5ever::interface::tree_builder::{ElementFlags, NodeOrText, TreeSink};
use html5ever::tendril::StrTendril;
use html5ever::tendril::TendrilSink;
use html5ever::{namespace_url, ns, parse_document, Attribute, QualName};
use markup5ever::interface::ExpandedName;
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

/// A parsed HTML document with its DOM tree.
pub struct Document {
    /// All nodes indexed by ID.
    nodes: HashMap<NodeId, Node>,
    /// Tree structure (parent/child relationships).
    tree: Tree,
    /// Next node ID.
    #[allow(dead_code)]
    next_id: usize,
}

impl Document {
    /// Parse an HTML string into a Document.
    pub fn parse(html: &str) -> Self {
        let sink = DomSink::new();
        let tendril = StrTendril::from(html);
        let result = parse_document(sink, html5ever::ParseOpts::default())
            .one(tendril);
        result.into_document()
    }

    /// Create an empty document.
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            tree: Tree::new(),
            next_id: 0,
        }
    }

    /// Allocate a new node ID.
    #[allow(dead_code)]
    fn alloc_id(&mut self) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Get a node by ID.
    pub fn get_node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    /// Get the root node ID.
    pub fn root(&self) -> Option<NodeId> {
        self.tree.root()
    }

    /// Get the tree structure.
    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    /// Query selector (basic CSS selector support).
    ///
    /// Supports: tag name, `.class`, `#id`, `tag.class`, `tag#id`.
    pub fn query_selector(&self, selector: &str) -> Option<NodeId> {
        let root = self.tree.root()?;
        let mut result = None;
        self.query_selector_recursive(root, selector, &mut result);
        result
    }

    fn query_selector_recursive(
        &self,
        current: NodeId,
        selector: &str,
        result: &mut Option<NodeId>,
    ) {
        if result.is_some() {
            return;
        }

        if let Some(node) = self.nodes.get(&current) {
            if self.node_matches_selector(node, selector) {
                *result = Some(current);
                return;
            }
        }

        for &child in self.tree.children(current) {
            self.query_selector_recursive(child, selector, result);
        }
    }

    fn node_matches_selector(&self, node: &Node, selector: &str) -> bool {
        if let NodeType::Element {
            tag,
            attributes,
        } = &node.node_type
        {
            // ID selector: #foo
            if let Some(id) = selector.strip_prefix('#') {
                return attributes
                    .iter()
                    .any(|(k, v)| k == "id" && v == id);
            }

            // Class selector: .foo
            if let Some(class) = selector.strip_prefix('.') {
                return attributes.iter().any(|(k, v)| {
                    k == "class" && v.split_whitespace().any(|c| c == class)
                });
            }

            // Tag with class: tag.class
            if let Some(dot_pos) = selector.find('.') {
                let tag_part = &selector[..dot_pos];
                let class_part = &selector[dot_pos + 1..];
                return tag.eq_ignore_ascii_case(tag_part)
                    && attributes.iter().any(|(k, v)| {
                        k == "class" && v.split_whitespace().any(|c| c == class_part)
                    });
            }

            // Tag with ID: tag#id
            if let Some(hash_pos) = selector.find('#') {
                let tag_part = &selector[..hash_pos];
                let id_part = &selector[hash_pos + 1..];
                return tag.eq_ignore_ascii_case(tag_part)
                    && attributes
                        .iter()
                        .any(|(k, v)| k == "id" && v == id_part);
            }

            // Simple tag name
            return tag.eq_ignore_ascii_case(selector);
        }
        false
    }

    /// Query all matching nodes.
    pub fn query_selector_all(&self, selector: &str) -> Vec<NodeId> {
        let mut results = Vec::new();
        if let Some(root) = self.tree.root() {
            self.query_all_recursive(root, selector, &mut results);
        }
        results
    }

    fn query_all_recursive(&self, current: NodeId, selector: &str, results: &mut Vec<NodeId>) {
        if let Some(node) = self.nodes.get(&current) {
            if self.node_matches_selector(node, selector) {
                results.push(current);
            }
        }
        for &child in self.tree.children(current) {
            self.query_all_recursive(child, selector, results);
        }
    }

    /// Get the text content of the first matching node.
    pub fn query_text(&self, selector: &str) -> Option<String> {
        let node_id = self.query_selector(selector)?;
        let mut text = String::new();
        self.collect_text(node_id, &mut text);
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    /// Collect all text content from a node and its descendants.
    fn collect_text(&self, node_id: NodeId, text: &mut String) {
        if let Some(node) = self.nodes.get(&node_id) {
            if let NodeType::Text(t) = &node.node_type {
                text.push_str(t);
            }
        }
        for &child in self.tree.children(node_id) {
            self.collect_text(child, text);
        }
    }

    /// Get text content of a specific node.
    pub fn text_content(&self, node_id: NodeId) -> Option<String> {
        let mut text = String::new();
        self.collect_text(node_id, &mut text);
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    /// Convert the document to a simple Markdown representation.
    pub fn to_markdown(&self) -> String {
        let mut md = String::new();
        if let Some(root) = self.tree.root() {
            self.node_to_markdown(root, &mut md, 0);
        }
        md
    }

    fn node_to_markdown(&self, node_id: NodeId, md: &mut String, depth: usize) {
        if let Some(node) = self.nodes.get(&node_id) {
            match &node.node_type {
                NodeType::Text(t) => {
                    let trimmed = t.trim();
                    if !trimmed.is_empty() {
                        md.push_str(trimmed);
                        md.push(' ');
                    }
                }
                NodeType::Element { tag, .. } => {
                    let tag_lower = tag.to_lowercase();
                    match tag_lower.as_str() {
                        "h1" => {
                            md.push_str("\n# ");
                            self.write_children_text(node_id, md);
                            md.push_str("\n\n");
                        }
                        "h2" => {
                            md.push_str("\n## ");
                            self.write_children_text(node_id, md);
                            md.push_str("\n\n");
                        }
                        "h3" => {
                            md.push_str("\n### ");
                            self.write_children_text(node_id, md);
                            md.push_str("\n\n");
                        }
                        "h4" | "h5" | "h6" => {
                            md.push_str("\n#### ");
                            self.write_children_text(node_id, md);
                            md.push_str("\n\n");
                        }
                        "p" | "div" | "section" | "article" => {
                            for &child in self.tree.children(node_id) {
                                self.node_to_markdown(child, md, depth);
                            }
                            md.push('\n');
                        }
                        "a" => {
                            let href = node.get_attribute("href").unwrap_or("").to_string();
                            self.write_children_text(node_id, md);
                            if !href.is_empty() {
                                md.push_str(&format!("({href})"));
                            }
                        }
                        "li" => {
                            md.push_str("- ");
                            for &child in self.tree.children(node_id) {
                                self.node_to_markdown(child, md, depth + 1);
                            }
                            md.push('\n');
                        }
                        "br" => {
                            md.push('\n');
                        }
                        "img" => {
                            let alt = node.get_attribute("alt").unwrap_or("").to_string();
                            let src = node.get_attribute("src").unwrap_or("").to_string();
                            md.push_str(&format!("![{alt}]({src})"));
                        }
                        "strong" | "b" => {
                            md.push_str("**");
                            self.write_children_text(node_id, md);
                            md.push_str("**");
                        }
                        "em" | "i" => {
                            md.push('*');
                            self.write_children_text(node_id, md);
                            md.push('*');
                        }
                        "code" => {
                            md.push('`');
                            self.write_children_text(node_id, md);
                            md.push('`');
                        }
                        "pre" => {
                            md.push_str("\n```\n");
                            self.write_children_text(node_id, md);
                            md.push_str("\n```\n");
                        }
                        _ => {
                            // Generic: just recurse into children
                            for &child in self.tree.children(node_id) {
                                self.node_to_markdown(child, md, depth);
                            }
                        }
                    }
                    return; // Children already handled above
                }
                _ => {}
            }

            // For text nodes and unhandled, recurse into children
            for &child in self.tree.children(node_id) {
                self.node_to_markdown(child, md, depth);
            }
        }
    }

    fn write_children_text(&self, node_id: NodeId, text: &mut String) {
        for &child in self.tree.children(node_id) {
            if let Some(node) = self.nodes.get(&child) {
                if let NodeType::Text(t) = &node.node_type {
                    text.push_str(t);
                } else if node.is_element() {
                    self.write_children_text(child, text);
                }
            }
        }
    }

    /// Number of nodes in the document.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// html5ever TreeSink implementation for building our DOM
// ---------------------------------------------------------------------------

/// The handle type used by html5ever's tree builder.
type DomHandle = NodeId;

/// html5ever TreeSink that builds our Document.
///
/// Uses interior mutability (Cell/RefCell) since html5ever 0.29 TreeSink
/// methods take `&self`.
struct DomSink {
    /// All nodes indexed by ID.
    nodes: RefCell<HashMap<NodeId, Node>>,
    /// Element QualNames — leaked to provide 'static references for elem_name.
    /// This is a small memory leak acceptable for the parser lifetime,
    /// following the same pattern as html5ever's own noop-tree-builder example.
    elem_names: RefCell<HashMap<NodeId, &'static QualName>>,
    /// Tree structure (parent/child relationships).
    tree: RefCell<Tree>,
    /// Next node ID.
    next_id: Cell<usize>,
}

impl DomSink {
    fn new() -> Self {
        let sink = Self {
            nodes: RefCell::new(HashMap::new()),
            elem_names: RefCell::new(HashMap::new()),
            tree: RefCell::new(Tree::new()),
            next_id: Cell::new(0),
        };
        // Create the document root node
        let root_id = sink.alloc_id();
        sink.nodes
            .borrow_mut()
            .insert(root_id, Node::new(root_id, NodeType::Document));
        sink.tree.borrow_mut().set_root(root_id);
        sink
    }

    fn alloc_id(&self) -> NodeId {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        NodeId(id)
    }

    fn into_document(self) -> Document {
        Document {
            nodes: self.nodes.into_inner(),
            tree: self.tree.into_inner(),
            next_id: self.next_id.into_inner(),
        }
    }
}

impl TreeSink for DomSink {
    type Handle = DomHandle;
    type Output = Self;
    type ElemName<'a> = ExpandedName<'a>;

    fn finish(self) -> Self {
        self
    }

    fn get_document(&self) -> Self::Handle {
        self.tree.borrow().root().unwrap()
    }

    fn elem_name<'a>(&self, target: &'a Self::Handle) -> ExpandedName<'a> {
        let names = self.elem_names.borrow();
        if let Some(qname) = names.get(target) {
            return qname.expanded();
        }
        // Fallback — should not be reached in normal parsing
        static FALLBACK: std::sync::OnceLock<QualName> = std::sync::OnceLock::new();
        let fallback = FALLBACK.get_or_init(|| QualName {
            prefix: None,
            ns: ns!(html).into(),
            local: string_cache::Atom::from(""),
        });
        fallback.expanded()
    }

    fn create_element(
        &self,
        name: QualName,
        attrs: Vec<Attribute>,
        _flags: ElementFlags,
    ) -> Self::Handle {
        let id = self.alloc_id();
        let tag = name.local.to_string();
        let attributes = attrs
            .into_iter()
            .map(|a| (a.name.local.to_string(), a.value.to_string()))
            .collect();

        // Leak the QualName so we can return 'static references from elem_name.
        // This follows the same pattern as html5ever's noop-tree-builder example.
        let static_name: &'static QualName = Box::leak(Box::new(name));
        self.elem_names.borrow_mut().insert(id, static_name);

        self.nodes
            .borrow_mut()
            .insert(id, Node::new(id, NodeType::Element { tag, attributes }));
        id
    }

    fn create_comment(&self, text: StrTendril) -> Self::Handle {
        let id = self.alloc_id();
        self.nodes
            .borrow_mut()
            .insert(id, Node::new(id, NodeType::Comment(text.to_string())));
        id
    }

    fn append(&self, parent: &Self::Handle, child: NodeOrText<Self::Handle>) {
        match child {
            NodeOrText::AppendText(text) => {
                let id = self.alloc_id();
                self.nodes
                    .borrow_mut()
                    .insert(id, Node::new(id, NodeType::Text(text.to_string())));
                self.tree.borrow_mut().append_child(*parent, id);
            }
            NodeOrText::AppendNode(node) => {
                self.tree.borrow_mut().append_child(*parent, node);
            }
        }
    }

    fn append_before_sibling(&self, sibling: &Self::Handle, child: NodeOrText<Self::Handle>) {
        let parent = self.tree.borrow().parent(*sibling);
        if let Some(parent) = parent {
            self.append(&parent, child);
        }
    }

    fn append_based_on_parent_node(
        &self,
        element: &Self::Handle,
        _prev_element: &Self::Handle,
        child: NodeOrText<Self::Handle>,
    ) {
        self.append_before_sibling(element, child);
    }

    fn append_doctype_to_document(
        &self,
        name: StrTendril,
        _public: StrTendril,
        _system: StrTendril,
    ) {
        let id = self.alloc_id();
        self.nodes
            .borrow_mut()
            .insert(id, Node::new(id, NodeType::Doctype { name: name.to_string() }));
        if let Some(root) = self.tree.borrow().root() {
            self.tree.borrow_mut().append_child(root, id);
        }
    }

    fn remove_from_parent(&self, _target: &Self::Handle) {
        // Simplified: no-op
    }

    fn reparent_children(&self, _parent: &Self::Handle, _new_parent: &Self::Handle) {
        // Simplified: no-op
    }

    fn add_attrs_if_missing(&self, target: &Self::Handle, attrs: Vec<Attribute>) {
        let mut nodes = self.nodes.borrow_mut();
        if let Some(node) = nodes.get_mut(target) {
            if let NodeType::Element { attributes, .. } = &mut node.node_type {
                for attr in attrs {
                    let name = attr.name.local.to_string();
                    if !attributes.iter().any(|(k, _)| k == &name) {
                        attributes.push((name, attr.value.to_string()));
                    }
                }
            }
        }
    }

    fn mark_script_already_started(&self, _node: &Self::Handle) {
        // No-op
    }

    fn set_quirks_mode(&self, _mode: html5ever::interface::tree_builder::QuirksMode) {
        // No-op
    }

    fn same_node(&self, x: &Self::Handle, y: &Self::Handle) -> bool {
        x == y
    }

    fn parse_error(&self, _msg: Cow<'static, str>) {
        #[cfg(debug_assertions)]
        tracing::debug!("HTML parse error: {}", _msg);
    }

    fn create_pi(&self, target: StrTendril, data: StrTendril) -> Self::Handle {
        let id = self.alloc_id();
        self.nodes.borrow_mut().insert(
            id,
            Node::new(id, NodeType::Comment(format!("<?{} {}?>", target, data))),
        );
        id
    }

    fn get_template_contents(&self, target: &Self::Handle) -> Self::Handle {
        *target
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_html() {
        let html = "<html><head><title>Test</title></head><body><p>Hello</p></body></html>";
        let doc = Document::parse(html);
        assert!(doc.node_count() > 0, "document should have nodes");
        let title_text = doc.query_text("title");
        assert_eq!(
            title_text.as_deref(),
            Some("Test"),
            "title text should be extracted"
        );
    }

    #[test]
    fn test_parse_empty_input() {
        let doc = Document::parse("");
        // Should not panic; may still have a document root node from html5ever
        let _ = doc.node_count();
    }

    #[test]
    fn test_parse_malformed_html() {
        let html = "<div><span>unclosed";
        let doc = Document::parse(html);
        // Should not panic; html5ever is lenient
        assert!(doc.node_count() > 0, "malformed HTML should still produce nodes");
    }

    #[test]
    fn test_query_selector_by_tag() {
        let html = "<html><body><p>first</p><p>second</p></body></html>";
        let doc = Document::parse(html);
        let found = doc.query_selector("p");
        assert!(found.is_some(), "should find a <p> element");
        let node = doc.get_node(found.unwrap()).unwrap();
        assert_eq!(node.tag_name(), Some("p"));
    }

    #[test]
    fn test_query_selector_by_class() {
        let html = r#"<html><body><div class="foo">content</div></body></html>"#;
        let doc = Document::parse(html);
        let found = doc.query_selector(".foo");
        assert!(found.is_some(), "should find element with class .foo");
    }

    #[test]
    fn test_query_selector_by_id() {
        let html = r#"<html><body><span id="bar">text</span></body></html>"#;
        let doc = Document::parse(html);
        let found = doc.query_selector("#bar");
        assert!(found.is_some(), "should find element with id #bar");
    }

    #[test]
    fn test_query_selector_all() {
        let html = "<html><body><ul><li>a</li><li>b</li><li>c</li></ul></body></html>";
        let doc = Document::parse(html);
        let items = doc.query_selector_all("li");
        assert_eq!(items.len(), 3, "should find 3 <li> elements");
    }

    #[test]
    fn test_query_text() {
        let html = "<html><head><title>My Title</title></head><body></body></html>";
        let doc = Document::parse(html);
        let text = doc.query_text("title");
        assert_eq!(text.as_deref(), Some("My Title"));
    }

    #[test]
    fn test_to_markdown() {
        let html = r#"<html><body><h1>Heading</h1><p>Paragraph</p><ul><li>item</li></ul><a href="http://example.com">link</a></body></html>"#;
        let doc = Document::parse(html);
        let md = doc.to_markdown();
        assert!(md.contains('#'), "markdown should contain heading #");
        assert!(md.contains("- "), "markdown should contain list item -");
    }

    #[test]
    fn test_tree_traversal() {
        let html = "<html><head><title>T</title></head><body><p>A</p><p>B</p></body></html>";
        let doc = Document::parse(html);
        let root = doc.root().expect("document should have a root");
        let mut visited = Vec::new();
        doc.tree().traverse_dfs(root, &mut |id| {
            if let Some(node) = doc.get_node(id) {
                if let Some(tag) = node.tag_name() {
                    visited.push(tag.to_string());
                }
            }
        });
        // DFS: html comes before head/body, head before title, body before p's
        let html_idx = visited.iter().position(|t| t == "html").unwrap();
        let head_idx = visited.iter().position(|t| t == "head").unwrap();
        let body_idx = visited.iter().position(|t| t == "body").unwrap();
        let title_idx = visited.iter().position(|t| t == "title").unwrap();
        assert!(html_idx < head_idx, "html before head");
        assert!(html_idx < body_idx, "html before body");
        assert!(head_idx < title_idx, "head before title");
    }

    #[test]
    fn test_node_attributes() {
        let html = r#"<html><body><a href="http://example.com" class="cls">link</a></body></html>"#;
        let doc = Document::parse(html);
        let node_id = doc.query_selector("a").expect("should find <a>");
        let node = doc.get_node(node_id).unwrap();
        assert_eq!(node.get_attribute("href"), Some("http://example.com"));
        assert_eq!(node.get_attribute("class"), Some("cls"));
    }
}
